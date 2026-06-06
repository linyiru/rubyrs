# `undef name, ...` keyword desugars to `undef_method` — the
# `method_undefined` hook fires for each name (the part rubyrs
# models; actual removal is a documented Tier-1 no-op, so we don't
# assert respond_to? here). Discovery: P3 Jekyll spike —
# concurrent-ruby's `undef freeze` (pulled in by i18n).
class Foo
  def self.method_undefined(name)
    puts "undefined: #{name}"
  end
  def bar; end
  def baz; end
  undef bar
  undef baz
end

# multiple names in one statement, in declaration order.
class Multi
  def self.method_undefined(name)
    puts "multi-undef: #{name}"
  end
  def a; end
  def b; end
  def c; end
  undef a, b, c
end

# undef in a module body runs without error.
module M
  def to_freeze; end
  undef to_freeze
end
puts "module undef ok"

# String-name args via undef_method (the desugar target) also fire.
class ViaMethod
  def self.method_undefined(name); puts "via_method: #{name}"; end
  def q; end
  undef_method "q"
end
