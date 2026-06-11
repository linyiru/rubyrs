# Class/module receivers resolving through the class-OBJECT's own
# instance ancestry (Class -> Module -> Object -> Kernel), explicit
# and bare, with and without blocks — plus precedence: singleton
# wins over the meta chain; method_missing stays the terminal.
class Module
  def meta_helper(x)
    "Module##{x}"
  end
end
class Class
  def cls_only
    "Class-instance"
  end
end
module FooM
  def self.via_bare; meta_helper(1); end
end
class BarC; end
p FooM.via_bare
p FooM.meta_helper(2)
p BarC.meta_helper(3)
p BarC.cls_only
begin
  FooM.cls_only
rescue NoMethodError
  puts "module misses Class-only: ok"
end

module Kernel
  def kern_fn(y)
    "kern-#{y}"
  end
end
class KHost
  def self.go; kern_fn(7); end
end
p KHost.go

module IvarHost
  define_singleton_method(:setit) { instance_variable_set(:@v, 42) }
  define_singleton_method(:getit) { instance_variable_get(:@v) }
end
IvarHost.setit
p IvarHost.getit

class Module
  def with_block_helper
    yield 5
  end
end
class BHost
  def self.go
    with_block_helper { |n| n * 10 }
  end
end
p BHost.go
p BHost.with_block_helper { |n| n + 1 }

# Precedence: a singleton method shadows the meta-chain method.
class Module
  def shadowed; "meta"; end
end
module SHost
  def self.shadowed; "singleton"; end
end
p SHost.shadowed

# method_missing still terminal.
class MMHost
  def self.method_missing(n, *a)
    "mm-#{n}-#{a.length}"
  end
end
p MMHost.zorp(1, 2)
