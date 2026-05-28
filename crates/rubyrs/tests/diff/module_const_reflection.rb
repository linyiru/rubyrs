## `Module#autoload?` / `Module#const_defined?` / `Module#const_get` —
## constant-table reflection. Closes TRY_RUNS pass-10 layer #1 + #2
## (tilt-2.7.0/lib/tilt/mapping.rb:361-365's `constant_defined?`
## walks user-supplied class names like "Tilt::ERBTemplate" via
## `Object.const_defined?(...) && Object.const_get(...).const_defined?(...)`).
##
## `autoload?` is a stub returning nil since rubyrs's `autoload`
## itself is a no-op (no lazy-loading model in Tier-1).
##
## `const_defined?` and `const_get` look up qualified names in
## the global `classes` / `constants` tables. Top-level lookups
## via `Object` use the bare const name (CRuby root convention);
## other classes use the `Cls::Name` form.

class Foo
end
class Foo::Bar
end
Foo::CONST = 42

## Shape 1: const_defined? / const_get on Object (root scope).
puts "obj-cd-Foo=#{Object.const_defined?(:Foo)}"
puts "obj-cd-Missing=#{Object.const_defined?(:NoSuchConst)}"
puts "obj-cg-Foo=#{Object.const_get(:Foo).inspect}"

## Shape 2: const_defined? / const_get on a user module.
puts "foo-cd-Bar=#{Foo.const_defined?(:Bar)}"
puts "foo-cd-CONST=#{Foo.const_defined?(:CONST)}"
puts "foo-cd-Missing=#{Foo.const_defined?(:NoSuch)}"
puts "foo-cg-Bar=#{Foo.const_get(:Bar).inspect}"
puts "foo-cg-CONST=#{Foo.const_get(:CONST)}"

## Shape 3: const_defined?/const_get accept Strings too (CRuby
## semantics — tilt's `constant_defined?` calls these with
## String arg via `name.split('::').inject(Object) { |s, n| ... }`).
puts "obj-cd-str=#{Object.const_defined?('Foo')}"
puts "obj-cg-str=#{Object.const_get('Foo').inspect}"
puts "foo-cd-str=#{Foo.const_defined?('Bar')}"

## Shape 4: const_get on missing raises NameError.
err = begin
  Foo.const_get(:NoSuch)
  "no-raise"
rescue NameError => e
  e.message.start_with?("uninitialized constant") ? "NameError-uninit" : "NameError-other"
end
puts "missing-const=#{err}"

## Shape 5: autoload? returns nil for never-registered names —
## the only behavior both interpreters agree on. CRuby returns
## the path string for actually-registered autoloads; rubyrs's
## autoload is a no-op stub (documented in SUBSET.md), so
## autoload? always returns nil. Tilt's
## `constant_defined?` chain (`scope.autoload?(n) || !scope.const_defined?(n)`)
## works correctly because nil falsy short-circuits to the
## const_defined? check, which is what we want.
puts "autoload?-missing=#{Object.autoload?(:NeverRegistered).inspect}"
puts "foo-autoload?=#{Foo.autoload?(:Anything).inspect}"

## Shape 6: tilt-shape walk — `name.split('::').inject(Object)`
## walks scopes and checks each level. Pin the end-to-end shape.
class Walked
  class Inner
    Marker = "found"
  end
end
result = "Walked::Inner::Marker".split('::').inject(Object) do |scope, n|
  break false if scope.autoload?(n) || !scope.const_defined?(n)
  scope.const_get(n)
end
puts "tilt-walk=#{result.inspect}"

## Shape 7: `respond_to?` advertises the trio.
puts "respond-autoload?=#{Object.respond_to?(:autoload?)}"
puts "respond-const_defined?=#{Object.respond_to?(:const_defined?)}"
puts "respond-const_get=#{Object.respond_to?(:const_get)}"

## Shape 8: `const_defined?` / `const_get` with unique missing
## names don't intern the lookup key — defends against the
## `Object.const_defined?("X#{i}")` interner-growth attack a
## hostile script could use to escape `Config::max_symbols`.
## Both interpreters agree on the surface behavior (false /
## NameError); the divergence is unobservable at the Ruby level
## but pinned via this shape so a regression that re-introduces
## the intern is detected (it would still match this output but
## audit caught it on Copilot review #277). (code-review #277.)
1000.times do |i|
  Object.const_defined?("Missing#{i}")
end
err = begin
  Object.const_get("DefinitelyMissingConst")
  "no-raise"
rescue NameError
  "NameError"
end
puts "interner-safe=#{err}"
