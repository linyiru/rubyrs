# `const_defined?` must raise `NameError: wrong constant name` for a
# syntactically-invalid name even when it starts with an uppercase letter
# (e.g. "Foo-bar") and was never interned. The never-interned fast-undefined
# path used to gate only on `starts_with(uppercase)`, so an invalid uppercase
# name slipped through and returned `false` instead of raising. Building the
# name at runtime (concat) keeps it out of the interner, exercising that path.
# (zeitwerk test_cpath_expected_at, via ConstantPathValidator#validate!, which
# is `Module.new.const_defined?(cname, false)`.)
mod = Module.new

bad = "Foo" + "-" + "bar"   # runtime-built → never interned as a symbol
begin
  p mod.const_defined?(bad, false)
rescue NameError => e
  puts "NameError: #{e.message}"
end

# A valid, never-interned name is genuinely undefined → false (no raise).
good = "Zz" + "Never" + "Interned"
p mod.const_defined?(good, false)

# Lowercase-first invalid name still raises (unchanged behavior).
begin
  p mod.const_defined?("foo" + "bar", false)
rescue NameError => e
  puts "NameError: #{e.message}"
end
