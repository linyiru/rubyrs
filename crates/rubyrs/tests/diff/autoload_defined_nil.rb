# Module#autoload? returns nil once the constant is actually defined,
# even if an autoload entry lingers (re-arming autoload on a loaded
# const doesn't take effect). Tilt's constant_defined? gates on this.
module M; end
class M::Foo; end
M.autoload(:Foo, "/nonexistent")
p M.autoload?(:Foo)
p M.const_defined?(:Foo)
p defined?(M::Foo) ? "constant" : nil

# a genuinely-pending autoload still reports its path
module N
  autoload :Bar, "/some/path/bar"
end
p N.autoload?(:Bar)
